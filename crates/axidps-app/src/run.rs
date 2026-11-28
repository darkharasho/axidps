//! The launcher's long-running loop and its one-shot game-folder commands.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::autostart::{self, Lifetime, Starter};
use crate::config::{self, GameState, Store};
use crate::firstrun;
use crate::game::gw2_running;
use crate::gamedir::{self, Outcome};
use crate::gw2path;
use crate::layout::Layout;
use crate::lifetime::{self, GameWatch, Verdict};
use crate::supervise::{Child, Supervisor};
use crate::update::Updater;

/// How often the game folder is checked.
const CHECK_EVERY: Duration = Duration::from_secs(15);
/// How often the supervisor and the updater run.
const TICK: Duration = Duration::from_secs(1);
/// How long the loop sleeps between looks at `stop` (and, on Windows, the tray).
#[cfg(windows)]
const STEP: Duration = Duration::from_millis(100);
#[cfg(not(windows))]
const STEP: Duration = Duration::from_secs(1);

/// The core child's name in the supervisor.
const CORE: &str = "axidps-core";

/// Core's arguments. Without a log folder it waits for one rather than exiting (it would be
/// restarted in a loop); with one, it writes there.
fn core_args(stop_file: &Path, log_dir: Option<&Path>) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "--stop-file".into(),
        stop_file.into(),
        "--wait-for-log-dir".into(),
    ];
    if let Some(dir) = log_dir {
        args.push("--log-dir".into());
        args.push(dir.into());
    }
    args
}

/// The log folder in the Wine prefix the game runs from, on Linux. `None` leaves core to its
/// own lookup (Documents on Windows, the default Steam Proton prefix on Linux).
fn core_log_dir(game: Option<&Path>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let _ = game;
        None
    }
    #[cfg(not(windows))]
    {
        let user = std::env::var("USER").ok();
        gw2path::prefix_log_dir(game?, &home(), user.as_deref())
    }
}

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The game folder: the saved path, then AxiAM's, AxiOM's, the registry's, then the usual
/// places. A newly found path is saved in the config.
fn resolve_game(store: &Store) -> Option<PathBuf> {
    let mut cfg = store.load_config();
    let home = home();
    let axiam = std::fs::read_to_string(gw2path::axiam_config_path(&home, cfg!(windows)))
        .ok()
        .and_then(|text| gw2path::axiam_gw2_path(&text));
    // AxiOM's own game folder setting, then where Windows recorded the install, then the
    // usual places.
    let mut candidates: Vec<PathBuf> = gw2path::axiom_config_paths(&home, cfg!(windows))
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|text| gw2path::axiom_gw2_path(&text))
        .collect();
    candidates.extend(gw2path::registry_candidates());
    candidates.extend(gw2path::candidates(&home, cfg!(windows)));
    let found = gw2path::resolve(cfg.gw2_path.as_deref(), axiam, &candidates)?;
    if cfg.gw2_path.as_deref() != Some(found.as_path()) {
        cfg.gw2_path = Some(found.clone());
        if let Err(e) = store.save_config(&cfg) {
            eprintln!("axidps: cannot save the game folder: {e}");
        }
    }
    Some(found)
}

/// Asks the user for the game folder, unless prompts are off, and saves an accepted pick.
fn ask_user(store: &Store) -> Option<PathBuf> {
    if firstrun::silent() {
        return None;
    }
    let game = firstrun::ask_gw2_folder()?;
    let mut cfg = store.load_config();
    cfg.gw2_path = Some(game.clone());
    if let Err(e) = store.save_config(&cfg) {
        eprintln!("axidps: cannot save the game folder: {e}");
    }
    Some(game)
}

/// The marker the DLL keeps while the game runs under Wine. `None` on Windows, where the DLL
/// starts the launcher itself.
fn game_marker() -> Option<PathBuf> {
    cfg!(unix)
        .then(|| axidps_protocol::paths::game_marker_path(axidps_protocol::v2::RingHost::Unix))
}

/// Deletes the game marker a crashed game left behind; otherwise the path unit would start the
/// launcher again as soon as it exits.
fn remove_marker(marker: &Path) {
    match std::fs::remove_file(marker) {
        Ok(()) => eprintln!("axidps: removed the stale {}", marker.display()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("axidps: cannot remove {}: {e}", marker.display()),
    }
}

/// Dispatches the window messages waiting for this thread (the tray icon's).
#[cfg(windows)]
fn pump_messages() {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid MSG for the calls; no window filter.
    unsafe {
        while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Runs `ensure` and keeps what it learned. The caller has already checked the game is not
/// running.
struct Keeper<'a> {
    store: &'a Store,
    layout: &'a Layout,
    version: &'a str,
    state: GameState,
    save_failed: bool,
    logged_missing_dll: bool,
    logged_elevation: bool,
    logged_kept_newer: bool,
    logged_granted_unwritable: bool,
    elevation_requested: bool,
    /// Starts the elevated grant (`wait`: and waits for it); replaced in tests.
    elevate: fn(&Path, bool) -> io::Result<bool>,
}

/// The platform's elevated `--grant-game-folder`; nothing to ask for off Windows.
fn platform_elevate(game: &Path, wait: bool) -> io::Result<bool> {
    #[cfg(windows)]
    {
        elevate::grant_game_folder(game, wait)
    }
    #[cfg(not(windows))]
    {
        let _ = (game, wait);
        Ok(false)
    }
}

impl<'a> Keeper<'a> {
    fn new(store: &'a Store, layout: &'a Layout, version: &'a str) -> Self {
        Keeper {
            store,
            layout,
            version,
            state: store.load_state(),
            save_failed: false,
            logged_missing_dll: false,
            logged_elevation: false,
            logged_kept_newer: false,
            logged_granted_unwritable: false,
            elevation_requested: false,
            elevate: platform_elevate,
        }
    }

    /// One `ensure`. The state is saved whether it succeeded or not, so a moved file is never
    /// left unrecorded.
    fn ensure_once(&mut self, game: &Path) -> Option<Outcome> {
        if !self.layout.dll.exists() {
            if !self.logged_missing_dll {
                self.logged_missing_dll = true;
                eprintln!(
                    "axidps: {} is missing; leaving the game folder alone",
                    self.layout.dll.display()
                );
            }
            return None;
        }
        // Reload: the elevated `--grant-game-folder` process writes `granted` behind our back.
        self.state = self.store.load_state();
        // A grant recorded without its folder becomes this folder's, and is saved so below.
        let _ = self.state.granted_to(game);
        let result = gamedir::ensure(game, &self.layout.dll, self.version, &mut self.state);
        if let Err(e) = self.store.save_state(&self.state) {
            self.save_failed = true;
            eprintln!("axidps: cannot save state: {e}");
        }
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                eprintln!("axidps: cannot update {}: {e}", game.display());
                return None;
            }
        };
        match outcome {
            Outcome::Unchanged => {}
            Outcome::NeedsElevation => {
                if !self.logged_elevation {
                    self.logged_elevation = true;
                    eprintln!("axidps: {} needs administrator access", game.display());
                }
            }
            Outcome::KeptNewer => {
                if !self.logged_kept_newer {
                    self.logged_kept_newer = true;
                    let installed = match gamedir::classify(&game.join("d3d11.dll")) {
                        Ok(gamedir::DllKind::Ours(v)) => v,
                        _ => "?".into(),
                    };
                    eprintln!(
                        "axidps: the game folder has axidps DLL {installed}, not older than this app's {}; leaving it (update this app if it is older)",
                        self.version
                    );
                }
            }
            other => eprintln!("axidps: game folder {other:?}"),
        }
        Some(outcome)
    }

    /// Whether a `NeedsElevation` should ask for administrator access. Not when the grant was
    /// already obtained (by the installer's `--install-game`, or an earlier start): asking again
    /// would show a second UAC prompt for a grant that is already in place. That case is logged
    /// once instead.
    fn wants_elevation(&mut self, game: &Path) -> bool {
        if !self.state.granted_to(game) {
            return true;
        }
        if !self.logged_granted_unwritable {
            self.logged_granted_unwritable = true;
            eprintln!(
                "axidps: access to {} was granted earlier but it is still not writable; not asking again",
                game.display()
            );
        }
        false
    }

    /// Asks for administrator access to `game`, once per process and only if it was never
    /// granted. With `wait`, returns once the elevated process has finished, and says whether
    /// it granted access; without, returns at once (false).
    fn request_elevation(&mut self, game: &Path, wait: bool) -> bool {
        if self.elevation_requested || !self.wants_elevation(game) {
            return false;
        }
        self.elevation_requested = true;
        match (self.elevate)(game, wait) {
            Ok(granted) => granted,
            Err(e) => {
                eprintln!("axidps: cannot ask for administrator access: {e}");
                false
            }
        }
    }

    /// The game folder before the launcher quits with the game: one `ensure`, and if the folder
    /// needs administrator access, a request that is waited for and then a second `ensure`. Asked
    /// again even if this process asked before without waiting: once it is gone, nothing
    /// installs until the game has started once more.
    fn ensure_before_quit(&mut self, game: &Path) {
        if self.ensure_once(game) == Some(Outcome::NeedsElevation) {
            self.elevation_requested = false;
            if self.request_elevation(game, true) {
                self.ensure_once(game);
            }
        }
    }
}

/// A file in the config folder that asks the running launcher to stop as an update does: core
/// finishes the fight in progress, then everything exits. The Windows installer and uninstaller
/// create it (with core's own stop file) before falling back to killing the processes.
const LAUNCHER_STOP: &str = "launcher.stop";

/// Whether `file` asks this launcher to stop; it is removed, so it is acted on once.
fn stop_requested(file: &Path) -> bool {
    if !file.exists() {
        return false;
    }
    let _ = std::fs::remove_file(file);
    true
}

/// How long a launcher started by an update waits for the old one to let go of the lock.
const AFTER_UPDATE_WAIT: Duration = Duration::from_secs(60);

/// Whether a launcher that did not get the lock tries again: one an update started, until
/// `deadline`; one the path unit started (`from_game`), while the game marker exists.
fn keep_waiting(now: Instant, deadline: Instant, from_game: bool, marker_up: bool) -> bool {
    now < deadline || (from_game && marker_up)
}

/// Whether the DLL in the game folder `game` tells the launcher the game runs
/// ([`axidps_protocol::ident::CAP_STARTS_LAUNCHER`]). Read again only when the file changes.
#[derive(Default)]
struct DllCaps {
    seen: Option<(PathBuf, u64, Option<std::time::SystemTime>)>,
    starts_launcher: bool,
}

impl DllCaps {
    fn starts_launcher(&mut self, game: Option<&Path>) -> bool {
        let Some(game) = game else {
            return false;
        };
        let file = game.join("d3d11.dll");
        let Ok(meta) = std::fs::metadata(&file) else {
            self.seen = None;
            self.starts_launcher = false;
            return false;
        };
        let key = (file.clone(), meta.len(), meta.modified().ok());
        if self.seen.as_ref() != Some(&key) {
            self.starts_launcher = std::fs::read(&file).is_ok_and(|bytes| {
                axidps_protocol::ident::dll_has_cap(
                    &bytes,
                    axidps_protocol::ident::CAP_STARTS_LAUNCHER,
                )
            });
            self.seen = Some(key);
        }
        self.starts_launcher
    }
}

/// Whether a launcher that got the lock only after waiting should exit at once: one the path
/// unit started (`from_game`) whose game closed while another launcher ran. Not one an update
/// started: it has to rewrite the units for the new AppImage.
fn exit_after_wait(from_game: bool, after_update: bool, waited: bool, marker_up: bool) -> bool {
    from_game && !after_update && waited && !marker_up
}

/// The long-running loop. Returns when `stop` is set, when the game has closed (a launcher that
/// runs with the game), or when another launcher runs (at once, or after waiting for it to exit
/// when `after_update`, or while the game marker exists when `from_game`).
pub fn run(
    layout: &Layout,
    store: &Store,
    stop: &AtomicBool,
    after_update: bool,
    from_game: bool,
) -> io::Result<()> {
    let wait = if after_update {
        AFTER_UPDATE_WAIT
    } else {
        Duration::ZERO
    };
    let deadline = Instant::now() + wait;
    let marker = game_marker();
    let marker_up = || marker.as_deref().is_some_and(Path::exists);
    // Started by the path unit while another launcher runs (one started by hand): this one must
    // stay up while the marker exists, or the path unit would start it again at once.
    let mut waited = false;
    let lock = crate::instance::acquire_while(store.dir(), || {
        waited = true;
        keep_waiting(Instant::now(), deadline, from_game, marker_up())
    });
    let _instance = match lock {
        Ok(Some(_)) if exit_after_wait(from_game, after_update, waited, marker_up()) => {
            eprintln!("axidps: the game closed while another launcher ran; this copy exits");
            return Ok(());
        }
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            eprintln!("axidps: already running; this copy exits");
            return Ok(());
        }
        // Running unguarded beats not running at all.
        Err(e) => {
            eprintln!("axidps: cannot take the single-instance lock: {e}");
            None
        }
    };
    // Left from a stop request no launcher saw; it must not stop this one.
    let launcher_stop = store.dir().join(LAUNCHER_STOP);
    let _ = std::fs::remove_file(&launcher_stop);
    let version = axidps_protocol::ident::build_version();
    let log = crate::applog::init(store.dir(), version);
    // Before the overlay host's window first appears.
    #[cfg(target_os = "linux")]
    crate::kwin::ensure();
    // Only AxiOM's display needs it: a failure must not stop the supervising.
    if let Err(e) = config::write_axiom_version(store.dir(), version) {
        eprintln!("axidps: cannot write the axiom-version file: {e}");
    }
    let mut game = resolve_game(store);
    if game.is_none() {
        eprintln!("axidps: Guild Wars 2 not found; looking again every 15 s");
    }
    let mut keeper = Keeper::new(store, layout, version);
    // Core finishes the fight in progress when this file appears (see Supervisor::shutdown).
    let core_stop = store.dir().join("core.stop");
    let mut supervisor = Supervisor::new(vec![
        Child {
            name: CORE,
            program: layout.core.clone(),
            args: core_args(&core_stop, core_log_dir(game.as_deref()).as_deref()),
            stop_file: Some(core_stop.clone()),
        },
        Child {
            name: "overlay",
            program: layout.overlay.clone(),
            args: vec![layout.page.clone().into_os_string()],
            stop_file: None,
        },
    ]);
    let mut updater = Updater::new(Instant::now());
    #[cfg(windows)]
    let tray = crate::tray::Tray::new();
    let mut next_check = Instant::now();
    let mut next_tick = Instant::now();
    // The dialog is shown at most once per process.
    let mut asked = false;
    #[cfg(unix)]
    if from_game && autostart::session_env(|n| std::env::var_os(n)).is_empty() {
        eprintln!(
            "axidps: started by the game with no display (DISPLAY and WAYLAND_DISPLAY are unset in systemd --user); the overlay cannot open. Start AxiDPS once from AxiOM in your desktop session to fix this"
        );
    }
    let mut starter = Starter::new();
    let mut caps = DllCaps::default();
    let mut lifetime = starter.keep(caps.starts_launcher(game.as_deref()));
    let log_lifetime = |lifetime: Lifetime| {
        if lifetime == Lifetime::WithGame {
            eprintln!(
                "axidps: runs while Guild Wars 2 runs{}",
                if from_game {
                    " (started by the game)"
                } else {
                    ""
                }
            );
        }
    };
    log_lifetime(lifetime);
    let mut watch = GameWatch::new(lifetime, Instant::now());
    let mut next_game_look = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        #[cfg(windows)]
        {
            pump_messages();
            match tray.poll() {
                Some(crate::tray::TrayCmd::CheckUpdate) => updater.request_check(),
                Some(crate::tray::TrayCmd::OpenLogs) => crate::tray::open_logs(),
                Some(crate::tray::TrayCmd::Quit) => stop.store(true, Ordering::SeqCst),
                None => {}
            }
        }
        let now = Instant::now();
        if now >= next_check {
            next_check = now + CHECK_EVERY;
            if let Some(log) = &log {
                log.maintain();
            }
            if game.is_none() {
                game = resolve_game(store);
            }
            // The game's own prefix (Lutris, another Steam library) may appear or change. Core
            // is restarted for it only while the game is closed, so no fight is cut in two; a
            // change found while the game runs is applied once it closes (or when core next
            // restarts on its own).
            let running = gw2_running();
            let args = core_args(&core_stop, core_log_dir(game.as_deref()).as_deref());
            supervisor.set_args(CORE, args, !running);
            if let Some(dir) = &game
                && !running
                && keeper.ensure_once(dir) == Some(Outcome::NeedsElevation)
                // With the game, the quit path asks (and waits); asking here too would show a
                // second prompt while the first may still be up.
                && lifetime == Lifetime::Always
            {
                // Not waited for: the loop keeps supervising while the UAC prompt is up.
                keeper.request_elevation(dir, false);
            }
            // After `ensure`: the DLL it just installed may be the first that starts the
            // launcher.
            let now_lifetime = starter.keep(caps.starts_launcher(game.as_deref()));
            if now_lifetime != lifetime {
                lifetime = now_lifetime;
                log_lifetime(lifetime);
                watch = GameWatch::new(lifetime, now);
            }
        }
        if now >= next_game_look {
            next_game_look = now + lifetime::POLL;
            let verdict = watch.observe(now, gw2_running());
            if verdict != Verdict::Stay {
                eprintln!(
                    "axidps: {}; finishing up and quitting",
                    if verdict == Verdict::GameClosed {
                        "Guild Wars 2 closed"
                    } else {
                        "Guild Wars 2 did not start"
                    }
                );
                if finish_after_game(
                    game.as_deref(),
                    &mut keeper,
                    &mut updater,
                    &mut supervisor,
                    marker.as_deref(),
                ) {
                    break;
                }
                eprintln!("axidps: Guild Wars 2 is running again; staying");
                watch.reopened(Instant::now());
            }
        }
        if now >= next_tick {
            next_tick = now + TICK;
            if stop_requested(&launcher_stop) {
                eprintln!("axidps: asked to stop ({LAUNCHER_STOP})");
                break;
            }
            supervisor.tick(now);
            updater.tick(now, &mut supervisor);
            // After the children are up: the dialog blocks this thread until it is answered.
            if game.is_none() && !asked {
                asked = true;
                game = ask_user(store);
            }
        }
        std::thread::sleep(STEP);
    }
    supervisor.shutdown();
    Ok(())
}

/// Does what waits for the game to be closed before the launcher quits with it: the game
/// folder, the stale marker, then a waiting update (which exits this process). Returns whether
/// to quit: not when the game has started again meanwhile.
fn finish_after_game(
    game: Option<&Path>,
    keeper: &mut Keeper,
    updater: &mut Updater,
    supervisor: &mut Supervisor,
    marker: Option<&Path>,
) -> bool {
    if gw2_running() {
        return false;
    }
    if let Some(dir) = game {
        keeper.ensure_before_quit(dir);
    }
    if gw2_running() {
        return false;
    }
    if let Some(marker) = marker {
        remove_marker(marker);
    }
    updater.apply_pending(Instant::now(), supervisor);
    true
}

/// `--install-game`: one `ensure`, then done.
pub fn install_game(layout: &Layout, store: &Store) -> ExitCode {
    let Some(game) = resolve_game(store).or_else(|| ask_user(store)) else {
        println!("Guild Wars 2 was not found; the launcher asks for its folder at its next start.");
        return ExitCode::SUCCESS;
    };
    if gw2_running() {
        println!("Guild Wars 2 is running; the launcher installs when it closes.");
        return ExitCode::SUCCESS;
    }
    let version = axidps_protocol::ident::build_version();
    let mut keeper = Keeper::new(store, layout, version);
    match keeper.ensure_once(&game) {
        // Waited for, so the launcher the installer starts next finds the grant recorded and
        // the folder writable, and does not prompt a second time.
        Some(Outcome::NeedsElevation) => {
            if keeper.request_elevation(&game, true) {
                if keeper.ensure_once(&game) == Some(Outcome::NeedsElevation) {
                    println!(
                        "The game folder is still not writable; the launcher retries while it runs."
                    );
                }
            } else {
                println!(
                    "The game folder needs administrator access; the launcher finishes the install."
                );
            }
        }
        Some(_) => {}
        None => println!("The game folder was not changed; the launcher retries while it runs."),
    }
    // So the game can start the launcher even before the launcher's first run, if the DLL now
    // in the game folder can.
    Starter::new().keep(DllCaps::default().starts_launcher(Some(&game)));
    if keeper.save_failed {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Puts `game` back as it was, unless the game runs.
fn uninstall_in(game: &Path, store: &Store, game_running: bool) -> Result<(), String> {
    if game_running {
        return Err("Guild Wars 2 is running; close it and try again".into());
    }
    let mut state = store.load_state();
    let result = gamedir::uninstall(game, &mut state);
    if let Err(e) = store.save_state(&state) {
        eprintln!("axidps: cannot save state: {e}");
    }
    result.map_err(|e| match e.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "no permission to change {}; run axidps --install-game once to grant access, or run as administrator ({e})",
            game.display()
        ),
        _ => format!("cannot restore {}: {e}", game.display()),
    })
}

/// Disables the path unit, stops the other launchers, then removes what starts them (units,
/// autostart entry, launcher path). In that order: the path unit would start a stopped launcher
/// again while the game runs, a running launcher would put the files back within 15 s, and its
/// `Starter` would write them back in between.
fn quiet_launchers(
    disable: impl FnOnce(),
    stop_others: impl FnOnce(),
    remove_autostart: impl FnOnce() -> io::Result<()>,
) {
    disable();
    stop_others();
    if let Err(e) = remove_autostart() {
        eprintln!("axidps: cannot remove what starts the launcher: {e}");
    }
}

/// `--uninstall`.
pub fn uninstall(store: &Store) -> ExitCode {
    quiet_launchers(
        autostart::disable,
        crate::game::stop_other_launchers,
        autostart::remove,
    );
    let Some(game) = resolve_game(store) else {
        println!("Guild Wars 2 was not found; nothing to restore.");
        return ExitCode::SUCCESS;
    };
    match uninstall_in(&game, store, gw2_running()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("axidps: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// `--grant-game-folder <path> [--grant-user <name>]`: give that user (default: this one) write access, then record it.
#[cfg(windows)]
pub fn grant_game_folder(path: &Path, user: Option<&str>, store: &Store) -> ExitCode {
    let user = user
        .map(str::to_owned)
        .unwrap_or_else(elevate::current_user);
    // Grant on the folder that was checked, not on `path`, which could be relinked meanwhile.
    let real = match crate::gw2path::check_grant_target(path, &user) {
        Ok(real) => real,
        Err(e) => {
            eprintln!("axidps: not granting access: {e}");
            return ExitCode::FAILURE;
        }
    };
    match elevate::icacls_grant(&real, &user) {
        Ok(()) => {
            let mut state = store.load_state();
            state.record_grant(path);
            if let Err(e) = store.save_state(&state) {
                eprintln!("axidps: cannot save state: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("axidps: cannot grant access to {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

/// `--grant-game-folder` only exists on Windows.
#[cfg(not(windows))]
pub fn grant_game_folder(_path: &Path, _user: Option<&str>, _store: &Store) -> ExitCode {
    eprintln!("axidps: --grant-game-folder is only needed on Windows");
    ExitCode::FAILURE
}

/// `arg` as one Windows command-line argument. A trailing `\` is doubled so it cannot escape
/// the closing quote.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote_arg(arg: &str) -> String {
    let trailing = arg.len() - arg.trim_end_matches('\\').len();
    format!("\"{arg}{}\"", "\\".repeat(trailing))
}

#[cfg(windows)]
mod elevate {
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, PCWSTR, w};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000; // public-check:ok

    /// `DOMAIN\\user` when the domain is known, else the bare user name.
    pub fn current_user() -> String {
        let name = std::env::var("USERNAME").unwrap_or_default();
        match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
            _ => name,
        }
    }

    /// `icacls "<path>" /grant "<user>:(OI)(CI)M"`.
    pub fn icacls_grant(path: &Path, user: &str) -> io::Result<()> {
        if user.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no user name"));
        }
        let status = Command::new("icacls")
            .arg(path)
            .arg("/grant")
            .arg(format!("{user}:(OI)(CI)M"))
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("icacls failed ({status})")))
        }
    }

    /// Starts `axidps.exe --grant-game-folder <path>` elevated (a UAC prompt). With `wait`,
    /// waits for it and returns whether it exited 0 (access granted and recorded); without,
    /// returns `Ok(false)` once it has started.
    pub fn grant_game_folder(path: &Path, wait: bool) -> io::Result<bool> {
        let exe = std::env::current_exe()?;
        let file = HSTRING::from(exe.as_os_str());
        // Quoted so a path with spaces stays one argument.
        // The launcher's own identity: the elevating account may be a different one.
        let params = HSTRING::from(format!(
            "--grant-game-folder {} --grant-user {}",
            super::quote_arg(&path.to_string_lossy()),
            super::quote_arg(&current_user())
        ));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // SAFETY: `info` is initialised with its size, and every string outlives the call and is
        // NUL-terminated by HSTRING.
        unsafe { ShellExecuteExW(&mut info) }.map_err(io::Error::other)?;
        let process = info.hProcess;
        if process.is_invalid() {
            return Ok(false);
        }
        let mut code = 1u32;
        // SAFETY: `process` is the handle SEE_MASK_NOCLOSEPROCESS asked for; it is closed once.
        unsafe {
            if wait {
                WaitForSingleObject(process, INFINITE);
                let _ = GetExitCodeProcess(process, &mut code);
            }
            let _ = CloseHandle(process);
        }
        Ok(wait && code == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const OURS: &[u8] = b"MZ ours axidps-dll-version=0.2.0\0 tail";

    struct Env {
        _tmp: tempfile::TempDir,
        game: PathBuf,
        store: Store,
        layout: Layout,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("gw2");
        fs::create_dir_all(&game).unwrap();
        let layout = Layout::beside(&tmp.path().join("app"));
        fs::create_dir_all(layout.dll.parent().unwrap()).unwrap();
        let store = Store::new(tmp.path().join("cfg"));
        Env {
            _tmp: tmp,
            game,
            store,
            layout,
        }
    }

    #[test]
    fn ensure_saves_state_right_after_moving_arcdps() {
        let e = env();
        fs::write(&e.layout.dll, OURS).unwrap();
        fs::write(e.game.join("d3d11.dll"), b"arcdps").unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        assert_eq!(keeper.ensure_once(&e.game), Some(Outcome::Installed));
        assert!(e.store.load_state().moved_foreign);
        assert_eq!(keeper.ensure_once(&e.game), Some(Outcome::Unchanged));
    }

    #[test]
    fn uninstall_stops_launchers_before_removing_autostart() {
        let order = std::cell::RefCell::new(Vec::new());
        quiet_launchers(
            || order.borrow_mut().push("disable"),
            || order.borrow_mut().push("stop"),
            || {
                order.borrow_mut().push("autostart");
                Err(io::Error::other("ignored"))
            },
        );
        assert_eq!(*order.borrow(), ["disable", "stop", "autostart"]);
    }

    #[test]
    fn core_args_wait_and_take_the_prefix_log_dir() {
        let stop = Path::new("/c/core.stop");
        assert_eq!(
            core_args(stop, None),
            ["--stop-file", "/c/core.stop", "--wait-for-log-dir"]
        );
        assert_eq!(
            core_args(stop, Some(Path::new("/p/logs"))),
            [
                "--stop-file",
                "/c/core.stop",
                "--wait-for-log-dir",
                "--log-dir",
                "/p/logs"
            ]
        );
    }

    #[test]
    fn lock_wait_follows_the_deadline_and_the_marker() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(60);
        // An update's launcher waits until the deadline.
        assert!(keep_waiting(t0, later, false, false));
        assert!(!keep_waiting(later, later, false, false));
        // The game's launcher waits while the marker exists, and stops when it goes.
        assert!(keep_waiting(later, t0, true, true));
        assert!(!keep_waiting(later, t0, true, false));
        // A manual start never waits for the marker.
        assert!(!keep_waiting(later, t0, false, true));
    }

    #[cfg(unix)]
    #[test]
    fn quit_path_asks_again_and_waits_after_an_earlier_request() {
        use std::os::unix::fs::PermissionsExt;
        let e = env();
        fs::write(&e.layout.dll, OURS).unwrap();
        fs::set_permissions(&e.game, fs::Permissions::from_mode(0o555)).unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        if fs::write(e.game.join("probe"), b"").is_ok() {
            return; // running as root: nothing is denied
        }
        // The loop's request, not waited for.
        assert_eq!(keeper.ensure_once(&e.game), Some(Outcome::NeedsElevation));
        keeper.elevate = |_, _| Ok(false);
        keeper.request_elevation(&e.game, false);
        assert!(keeper.elevation_requested);
        // The quit path's: waited for. The fake "grants" by making the folder writable.
        thread_local!(static WAITED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
        keeper.elevate = |game, wait| {
            WAITED.with(|w| w.set(wait));
            fs::set_permissions(game, fs::Permissions::from_mode(0o755))?;
            Ok(true)
        };
        keeper.ensure_before_quit(&e.game);
        assert!(WAITED.with(|w| w.get()));
        assert_eq!(fs::read(e.game.join("d3d11.dll")).unwrap(), OURS);
    }

    #[test]
    fn dll_caps_follow_the_installed_file() {
        let e = env();
        let mut caps = DllCaps::default();
        assert!(!caps.starts_launcher(None));
        assert!(!caps.starts_launcher(Some(&e.game)));
        fs::write(e.game.join("d3d11.dll"), OURS).unwrap();
        assert!(!caps.starts_launcher(Some(&e.game)));
        fs::write(
            e.game.join("d3d11.dll"),
            b"MZ axidps-dll-version=0.3.0\0 axidps-dll-caps=starts-launcher\0 more",
        )
        .unwrap();
        assert!(caps.starts_launcher(Some(&e.game)));
    }

    #[test]
    fn a_game_started_copy_exits_if_the_game_closed_while_it_waited() {
        // from_game, after_update, waited, marker_up
        assert!(exit_after_wait(true, false, true, false));
        assert!(!exit_after_wait(true, false, true, true));
        assert!(!exit_after_wait(true, false, false, false));
        assert!(!exit_after_wait(false, false, true, false));
        // An update's new launcher must stay to rewrite the units.
        assert!(!exit_after_wait(true, true, true, false));
    }

    #[test]
    fn stale_marker_is_removed_and_a_missing_one_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("axidps-game");
        fs::write(&marker, b"").unwrap();
        remove_marker(&marker);
        assert!(!marker.exists());
        remove_marker(&marker);
    }

    #[test]
    fn game_marker_is_the_shared_one_on_linux() {
        #[cfg(unix)]
        assert_eq!(
            game_marker().as_deref(),
            Some(Path::new("/dev/shm/axidps-game"))
        );
    }

    #[test]
    fn stop_request_is_acted_on_once() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(LAUNCHER_STOP);
        assert!(!stop_requested(&file));
        fs::write(&file, b"").unwrap();
        assert!(stop_requested(&file));
        assert!(!file.exists());
        assert!(!stop_requested(&file));
    }

    #[test]
    fn quote_arg_survives_trailing_backslash() {
        assert_eq!(quote_arg(r"C:\GW2"), r#""C:\GW2""#);
        assert_eq!(quote_arg(r"C:\GW2\"), r#""C:\GW2\\""#);
    }

    #[test]
    fn ensure_keeps_granted_written_by_another_process() {
        let e = env();
        fs::write(&e.layout.dll, OURS).unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        // The elevated process records the grant after the keeper loaded its state.
        e.store
            .save_state(&GameState {
                granted: true,
                ..GameState::default()
            })
            .unwrap();
        keeper.ensure_once(&e.game);
        assert!(e.store.load_state().granted);
    }

    #[test]
    fn granted_folder_is_not_asked_for_again() {
        let e = env();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        assert!(keeper.wants_elevation(&e.game));
        // The installer's elevated `--grant-game-folder` recorded the grant.
        let mut st = GameState::default();
        st.record_grant(&e.game);
        e.store.save_state(&st).unwrap();
        fs::write(&e.layout.dll, OURS).unwrap();
        keeper.ensure_once(&e.game);
        assert!(!keeper.wants_elevation(&e.game));
        assert!(!keeper.request_elevation(&e.game, true));
        assert!(!keeper.elevation_requested);
    }

    #[test]
    fn an_old_grant_is_saved_as_this_folders() {
        let e = env();
        e.store
            .save_state(&GameState {
                granted: true,
                ..GameState::default()
            })
            .unwrap();
        fs::write(&e.layout.dll, OURS).unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        keeper.ensure_once(&e.game);
        assert_eq!(
            e.store.load_state().granted_for.as_deref(),
            Some(e.game.as_path())
        );
    }

    #[test]
    fn a_grant_for_another_folder_does_not_stop_the_prompt() {
        let e = env();
        let moved = e.game.parent().unwrap().join("Program Files/Guild Wars 2");
        let mut st = GameState::default();
        st.record_grant(&e.game);
        e.store.save_state(&st).unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        fs::write(&e.layout.dll, OURS).unwrap();
        fs::create_dir_all(&moved).unwrap();
        keeper.ensure_once(&moved);
        assert!(keeper.wants_elevation(&moved));
        assert!(!keeper.wants_elevation(&e.game));
    }

    #[test]
    fn missing_shipped_dll_skips_ensure() {
        let e = env();
        fs::write(e.game.join("d3d11.dll"), b"arcdps").unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        assert_eq!(keeper.ensure_once(&e.game), None);
        assert_eq!(fs::read(e.game.join("d3d11.dll")).unwrap(), b"arcdps");
        assert!(!e.store.load_state().moved_foreign);
    }

    #[test]
    fn uninstall_refuses_while_game_runs() {
        let e = env();
        fs::write(&e.layout.dll, OURS).unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        keeper.ensure_once(&e.game);
        assert!(uninstall_in(&e.game, &e.store, true).is_err());
        assert_eq!(fs::read(e.game.join("d3d11.dll")).unwrap(), OURS);
    }

    #[test]
    fn uninstall_restores_and_records() {
        let e = env();
        fs::write(&e.layout.dll, OURS).unwrap();
        fs::write(e.game.join("d3d11.dll"), b"arcdps").unwrap();
        let mut keeper = Keeper::new(&e.store, &e.layout, "0.2.0");
        keeper.ensure_once(&e.game);
        uninstall_in(&e.game, &e.store, false).unwrap();
        assert_eq!(fs::read(e.game.join("d3d11.dll")).unwrap(), b"arcdps");
        assert!(!e.store.load_state().moved_foreign);
    }
}
