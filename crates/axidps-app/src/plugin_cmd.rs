//! `axidps plugin check` and `axidps plugin preview`: tools for plugin authors that run and
//! exit without touching the launcher's supervisor, tray or single-instance lock.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use axidps_plugins::MANIFEST_FILE;
use axidps_plugins::manifest::problems;
use axidps_plugins::scan::read_manifest;
use axidps_plugins::serve;

#[derive(clap::Subcommand)]
pub enum Action {
    /// Check a plugin folder's manifest and list every problem.
    Check { folder: PathBuf },
    /// Serve the overlay page and a plugin in a normal browser, reloading on save.
    Preview {
        /// The plugin folder to preview.
        folder: Option<PathBuf>,
        #[arg(long, default_value_t = 8711)]
        port: u16,
        /// Do not open the browser.
        #[arg(long)]
        no_open: bool,
        /// The overlay page folder (default: the installed one).
        #[arg(long)]
        page: Option<PathBuf>,
        /// The plugin SDK folder (default: the installed one).
        #[arg(long)]
        sdk: Option<PathBuf>,
        /// The bundled plugins folder (default: the installed one).
        #[arg(long)]
        plugins: Option<PathBuf>,
    },
    /// Show your developer key's public half (made on first run), to request a developer pass.
    DevKey {
        /// Replace the key with a new one. This cancels any developer pass you already have.
        #[arg(long)]
        new: bool,
    },
}

pub fn run(action: Action) -> ExitCode {
    match action {
        Action::Check { folder } => {
            let code = check(&folder, &mut std::io::stdout());
            ExitCode::from(code)
        }
        Action::Preview {
            folder,
            port,
            no_open,
            page,
            sdk,
            plugins,
        } => preview(folder, port, no_open, page, sdk, plugins),
        Action::DevKey { new } => {
            let Some(config) = axidps_protocol::paths::config_dir() else {
                eprintln!("axidps: cannot find the config folder");
                return ExitCode::FAILURE;
            };
            ExitCode::from(dev_key(&config, new, &mut std::io::stdout()))
        }
    }
}

/// Prints `ok: <id> <version>` (returning 0) or one `problem: <line>` per problem (returning 1):
/// the manifest's, and each window entry that isn't a file in the folder.
pub fn check(folder: &Path, out: &mut impl Write) -> u8 {
    let result = match read_manifest(&folder.join(MANIFEST_FILE)) {
        Ok(bytes) => {
            let missing = missing_entries(&bytes, folder);
            match problems(&bytes, folder) {
                Ok(m) if missing.is_empty() => Ok(m),
                Ok(_) => Err(missing),
                Err(mut list) => {
                    // Entries the manifest check already called invalid aren't looked up.
                    let extra: Vec<String> = missing
                        .into_iter()
                        .filter(|l| {
                            let field = l.split(':').next().unwrap_or("");
                            !list.iter().any(|p| p.starts_with(&format!("{field}:")))
                        })
                        .collect();
                    list.extend(extra);
                    Err(list)
                }
            }
        }
        Err(e) => Err(vec![format!(
            "cannot read {MANIFEST_FILE} in {}: {e}",
            folder.display()
        )]),
    };
    match result {
        Ok(m) => {
            let _ = writeln!(out, "ok: {} {}", m.id, m.version);
            0
        }
        Err(list) => {
            for p in list {
                let _ = writeln!(out, "problem: {p}");
            }
            1
        }
    }
}

/// `windows[i].entry: "x.html" not found` for each window entry (a string) that doesn't resolve
/// to a file inside `folder`, as the host would serve it.
fn missing_entries(bytes: &[u8], folder: &Path) -> Vec<String> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let Some(windows) = v.get("windows").and_then(|w| w.as_array()) else {
        return Vec::new();
    };
    windows
        .iter()
        .enumerate()
        .filter_map(|(i, w)| {
            let entry = w.get("entry")?.as_str()?;
            serve::resolve(folder, entry)
                .is_none()
                .then(|| format!("windows[{i}].entry: {} not found", serde_json::json!(entry)))
        })
        .collect()
}

/// Prints `public key: <base64>` and how to ask for a pass, making the key if there is none.
/// With `new`, removes the old key first (after saying it cancels any pass). Returns the exit code.
pub fn dev_key(config: &Path, new: bool, out: &mut impl Write) -> u8 {
    use axidps_plugins::trust::{DEV_KEY_FILE, load_dev_key, make_dev_key};
    use base64::Engine;
    let path = config.join(DEV_KEY_FILE);
    if new && path.exists() {
        let _ = writeln!(out, "a new key cancels any developer pass you already have");
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!("axidps: cannot remove {}: {e}", path.display());
            return 1;
        }
    }
    let key = match load_dev_key(config) {
        Some(k) => k,
        None => {
            // A file that is there but unreadable as a key is replaced only with --new.
            if path.exists() {
                eprintln!(
                    "axidps: {} is not a valid key (use --new to replace it)",
                    path.display()
                );
                return 1;
            }
            match make_dev_key(config) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("axidps: {e}");
                    return 1;
                }
            }
        }
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    let _ = writeln!(out, "public key: {b64}");
    let _ = writeln!(
        out,
        "send this and your GitHub login to the axidps owner to request a developer pass"
    );
    0
}

fn preview(
    folder: Option<PathBuf>,
    port: u16,
    no_open: bool,
    page: Option<PathBuf>,
    sdk: Option<PathBuf>,
    plugins: Option<PathBuf>,
) -> ExitCode {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("overlay")))
        .unwrap_or_default();
    let config = crate::preview::Config {
        page: page.unwrap_or_else(|| installed.join("page")),
        sdk: sdk.unwrap_or_else(|| installed.join("sdk")),
        bundled: plugins.unwrap_or_else(|| installed.join("plugins")),
        folder,
    };
    for (what, dir) in [("page", &config.page), ("sdk", &config.sdk)] {
        if !dir.is_dir() {
            eprintln!(
                "axidps: the {what} folder {} does not exist (pass --{what} <dir>)",
                dir.display()
            );
            return ExitCode::FAILURE;
        }
    }
    let server = match crate::preview::start(config, port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("axidps: cannot serve on 127.0.0.1:{port}: {e}");
            return ExitCode::FAILURE;
        }
    };
    for line in &server.log {
        eprintln!("axidps: {line}");
    }
    let url = format!("http://127.0.0.1:{}/index.html?demo", server.port);
    println!("Previewing at {url}");
    println!("Press Ctrl-C to stop.");
    if !no_open && !open_browser(&url) {
        println!("Could not open a browser; open the address above yourself.");
    }
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .is_err()
    {
        eprintln!("axidps: cannot watch for Ctrl-C; close this window to stop");
    }
    let _ = rx.recv();
    ExitCode::SUCCESS
}

/// Opens `url` in the default browser; false when that failed.
fn open_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", ""]).arg(url);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_check(dir: &Path) -> (u8, String) {
        let mut out = Vec::new();
        let code = check(dir, &mut out);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn dev_key_is_made_once_and_stable() {
        let t = tempfile::tempdir().unwrap();
        let run = |new| {
            let mut out = Vec::new();
            assert_eq!(dev_key(t.path(), new, &mut out), 0);
            String::from_utf8(out).unwrap()
        };
        let first = run(false);
        assert!(first.starts_with("public key: "));
        assert!(first.contains("GitHub login"));
        assert_eq!(run(false), first);
        let hex = std::fs::read_to_string(t.path().join("dev-key")).unwrap();
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        let renewed = run(true);
        assert!(renewed.contains("cancels any developer pass"));
        assert_ne!(renewed.lines().nth(1), first.lines().next());
    }

    #[test]
    fn check_reports_all_problems() {
        let t = tempfile::tempdir().unwrap();
        let wins: Vec<String> = (1..=5)
            .map(|i| format!(r#"{{"id":"w{i}","title":"t","entry":"a.html"}}"#))
            .collect();
        std::fs::write(
            t.path().join(MANIFEST_FILE),
            format!(
                r#"{{"id":"BAD ID","name":"n","version":"1.0.0","api":1,"windows":[{}]}}"#,
                wins.join(",")
            ),
        )
        .unwrap();
        std::fs::write(t.path().join("a.html"), "x").unwrap();
        let (code, out) = run_check(t.path());
        assert_eq!(code, 1, "{out}");
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(out.lines().all(|l| l.starts_with("problem: ")), "{out}");
    }

    #[test]
    fn check_ok_and_missing() {
        let t = tempfile::tempdir().unwrap();
        let (code, out) = run_check(t.path());
        assert_eq!(code, 1);
        assert!(out.starts_with("problem: "), "{out}");
        std::fs::write(
            t.path().join(MANIFEST_FILE),
            r#"{"id":"ok.one","name":"n","version":"1.2.3","api":1,"windows":[{"id":"w","title":"t","entry":"a.html"}]}"#,
        )
        .unwrap();
        std::fs::write(t.path().join("a.html"), "x").unwrap();
        assert_eq!(run_check(t.path()), (0, "ok: ok.one 1.2.3\n".to_owned()));
    }

    #[test]
    fn check_reports_missing_entries() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(
            t.path().join(MANIFEST_FILE),
            r#"{"id":"ok.one","name":"n","version":"1.2.3","api":1,"windows":[
                {"id":"a","title":"t","entry":"a.html"},
                {"id":"b","title":"t","entry":"sub/b.html"},
                {"id":"c","title":"t","entry":"c.html"}]}"#,
        )
        .unwrap();
        std::fs::write(t.path().join("a.html"), "x").unwrap();
        std::fs::create_dir(t.path().join("c.html")).unwrap();
        let (code, out) = run_check(t.path());
        assert_eq!(code, 1, "{out}");
        assert_eq!(
            out,
            "problem: windows[1].entry: \"sub/b.html\" not found\n\
             problem: windows[2].entry: \"c.html\" not found\n",
            "a folder is not a file"
        );
    }

    #[test]
    fn check_reports_missing_entries_beside_other_problems_once() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(
            t.path().join(MANIFEST_FILE),
            r#"{"id":"BAD ID","name":"n","version":"1.2.3","api":1,"windows":[
                {"id":"a","title":"t","entry":"gone.html"},
                {"id":"b","title":"t","entry":"../x.html"}]}"#,
        )
        .unwrap();
        let (code, out) = run_check(t.path());
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("problem: id: "), "{out}");
        assert!(out.contains("problem: windows[0].entry: \"gone.html\" not found"), "{out}");
        assert_eq!(out.matches("windows[1].entry").count(), 1, "{out}");
    }

    #[test]
    fn check_reads_a_capped_manifest() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(MANIFEST_FILE), vec![b' '; 200 * 1024]).unwrap();
        let (code, out) = run_check(t.path());
        assert_eq!(code, 1);
        assert!(out.contains("at most"), "{out}");
    }
}
