//! Is Guild Wars 2 running?

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// Whether a process named `Gw2-64.exe` (case-insensitive) is running. Under Proton the
/// process keeps that name. Also true when the process list cannot be read.
pub fn gw2_running() -> bool {
    // If the process list cannot be read, assume the game runs: the caller then leaves the
    // game folder alone instead of replacing files under a live game.
    let Ok(mut sys) = System::new() else {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("axidps: cannot list processes; taking Guild Wars 2 as running");
        }
        return true;
    };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.processes().values().any(|p| {
        p.name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("Gw2-64.exe"))
    })
}

/// The pids in `procs` (pid, process name) that are another launcher: named `axidps` or
/// `axidps.exe`, not `own`. Names only, so the AppImage's own helper process never matches.
fn launcher_pids(procs: &[(u32, String)], own: u32) -> Vec<u32> {
    procs
        .iter()
        .filter(|(pid, name)| {
            *pid != own
                && (name.eq_ignore_ascii_case("axidps") || name.eq_ignore_ascii_case("axidps.exe"))
        })
        .map(|(pid, _)| *pid)
        .collect()
}

/// Stops every other running launcher, so it cannot put the game files back. Failures are
/// printed; the caller goes on.
pub fn stop_other_launchers() {
    let Ok(mut sys) = System::new() else {
        eprintln!("axidps: cannot list processes; a running launcher is left alone");
        return;
    };
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing(),
    );
    let procs: Vec<(u32, String)> = sys
        .processes()
        .iter()
        .map(|(pid, p)| (pid.as_u32(), p.name().to_string_lossy().into_owned()))
        .collect();
    for pid in launcher_pids(&procs, std::process::id()) {
        let Some(p) = sys.process(sysinfo::Pid::from_u32(pid)) else {
            continue;
        };
        if p.kill() {
            // Give it a moment to go, so its loop cannot run once more.
            p.wait();
        } else {
            eprintln!("axidps: cannot stop the running launcher (pid {pid})");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::launcher_pids;

    #[test]
    fn launcher_pids_match_exe_name_only() {
        let procs: Vec<(u32, String)> = [
            (1, "axidps"),
            (2, "axidps.exe"),
            (3, "AXIDPS.EXE"),
            (4, "axidps-core"),
            (5, "axidps-0.2.0.AppImage"),
            (6, "bash"),
            (7, "axidps"),
        ]
        .iter()
        .map(|(p, n)| (*p, n.to_string()))
        .collect();
        assert_eq!(launcher_pids(&procs, 7), vec![1, 2, 3]);
    }

    #[test]
    fn gw2_not_running_in_tests() {
        assert!(!super::gw2_running());
    }
}
